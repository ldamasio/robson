#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <string.h>
#include "include/help.h"
#include "include/report.h"
#include "include/say.h"
#include "include/buy.h"
#include "include/sell.h"
int main(int argc, char *argv[]) {
	if (argc<2) {
		printf("Welcome to Robson 0.01\n"); 
	} else {
		if (strcmp(argv[1], "--help")==0) {
			rbs_openscreen_help(); return 0;
		} 
		else if (strcmp(argv[1], "--report")==0) {
			rbs_openscreen_report(); return 0;
		} 
		else if (strcmp(argv[1], "--say")==0) {
			rbs_openscreen_say(); return 0;
		} 
		else if (strcmp(argv[1], "--buy")==0) {
			rbs_openscreen_buy(); return 0;
		} 
		else if (strcmp(argv[1], "--sell")==0) {
			rbs_openscreen_sell(); return 0;
		} 
		else {
			printf("Type --help. Invalid argument %s\n", argv[1]);
		}
	}
}
